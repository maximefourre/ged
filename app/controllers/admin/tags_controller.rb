class Admin::TagsController < Admin::BaseController
  def index
    @tags = Tag.order(:name)
    @tag = Tag.new
  end

  def create
    tag = Tag.new(params.require(:tag).permit(:name))
    if tag.save
      redirect_to admin_tags_path, notice: "Tag created."
    else
      redirect_to admin_tags_path, alert: tag.errors.full_messages.join(", ")
    end
  end

  def destroy
    Tag.find(params[:id]).destroy
    redirect_to admin_tags_path, notice: "Tag deleted."
  end
end
